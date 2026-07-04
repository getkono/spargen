# Spargen — Product Requirements Document

**A compile-time-correct Rust client generator for OpenAPI 3.1.x. Nothing else.**

| | |
|---|---|
| Status | Draft for implementation |
| Scope | OpenAPI 3.1.x → Rust HTTP client code, exclusively |
| Naming note | `spargen` verified available on crates.io as of 2026-07-04 |

The name: a *spar* is the single load-bearing beam of an aircraft wing — sized on the drawing board, carrying the entire span in flight with nothing propping it up. That is the product: everything structural is decided at generation time; nothing is interpreted at runtime. Spec in, spar out.

---

## 1. Background and case

Most of the modern Rust server ecosystem emits OpenAPI **3.1** (utoipa, aide, poem-openapi, and increasingly everything downstream of JSON Schema 2020-12). The ecosystem-default client generator, Progenitor, targets **3.0.x** and never crossed the gap — 3.1 is not a patch release of 3.0; it replaces OpenAPI's bespoke schema dialect with real JSON Schema 2020-12 (`nullable` → type arrays, `exclusiveMinimum` becomes a number, `$defs`, `prefixItems`, `const`, etc.). The workarounds in the wild are grim: published crates document literally running `sed -i 's/openapi: 3.1.0/openapi: 3.0.0/'` on upstream specs before feeding them to Progenitor. That "works" only by accident and silently miscompiles any schema that uses 3.1 semantics.

The remaining options are worse: `openapi-generator`'s Rust output is template-driven, unidiomatic, and fails opaquely; hand-writing clients discards the spec as a source of truth. There is no Rust generator that (a) speaks 3.1 natively, (b) fails loudly and precisely on what it doesn't support, and (c) treats binary size and dependency hygiene as first-class constraints. Spargen is that tool, and only that tool.

---

## 2. Product definition

Spargen consumes an OpenAPI 3.1.x document at build/generation time and produces idiomatic, deterministic Rust source: typed models, a `Client`, one method per operation, and typed errors. Generated code compiles or generation fails — with a diagnostic that names the exact spec construct, its JSON Pointer, and its source location. There is no runtime spec interpretation, ever.

### 2.1 Packaging: one crate, freestanding output

Opinionated decision: **`spargen` is a single crate (library + binary), and no Spargen crate ever appears in a consumer's runtime dependency tree — not even a companion runtime crate.** Generated output is *freestanding*: the shared support code (dispatch routines, the error taxonomy, `ResponseValue<T>`) is emitted as a private `support` module inside the generated code itself.

One pushback on "the Progenitor pattern": Progenitor's default arrangement leaves `progenitor-client` in the consumer's *runtime* graph and version-couples every generated client to it — mismatches between generator and runtime-crate versions produce exactly the confusing failures this project opposes. Its *inline* mode (`--include-client`) is the good idea buried as an option; Spargen adopts it as the **only** mode. This is stronger than the single-crate proposal alone, and it is what actually delivers "dependent libraries have minimal dependencies":

- Runtime dependencies of generated code are exactly `reqwest` (no default features), `serde`, `serde_json`, `bytes`, plus the optional feature-gated type crates of §6.2 — every one a dependency most applications already carry, and zero project-specific crates.
- Spargen's own machinery (`syn`, `quote`, `prettyplease`, span-preserving parser, `clap`) exists only at generation time: a `[build-dependencies]` entry in `build.rs` mode, or **absent from `Cargo.toml` entirely** in the recommended CLI mode (installed tool + CI gate).
- One crate, one version: the generator↔runtime version-skew class of bugs is eliminated by construction. The `spargen` version is stamped into a header comment of generated output for provenance.

Accepted trade-offs, stated plainly:

1. A binary linking N generated clients duplicates the support module N×. Bounded by keeping the module deliberately tiny and non-generic (§6.2); an opt-in shared-runtime mode is explicitly deferred and non-binding (D14, §10.1); size CI measures the actual duplication cost so any revisit argues from a number.
2. Fixes to support code ship by regeneration, not `cargo update`. Mitigated by deterministic output plus the `--check` drift gate, which make regeneration a routine one-command operation.
3. Each client's `Error` type is nominally distinct — identical in shape across all Spargen output, but not unifiable across clients without caller-side conversion.

Crate shape and usage:

| Item | Details |
|---|---|
| `spargen` (lib) | Parser (span-preserving) → validator → IR → codegen; the public API used from `build.rs` |
| `spargen` (bin) | `spargen generate`, `spargen check`, `spargen explain E###`; gated behind a default-on `cli` feature (`[[bin]] required-features = ["cli"]`) so `cargo install spargen` works out of the box while CLI-only deps stay out of library builds |

1. **CLI, checked-in output (primary, recommended):** Spargen appears nowhere in `Cargo.toml`; developers and CI run the installed binary.
2. **`build.rs`:** `[build-dependencies] spargen = { version = "…", default-features = false }` — a build-time dependency only, per the dev/build-dependency pattern proposed.

### 2.2 Generation modes

1. **CLI, checked-in output (primary, recommended).** Emits a module or a standalone publishable crate. Best debuggability (errors point at visible code, IDEs work), nothing generator-related in the consumer's build graph, and `spargen generate --check` fails CI when checked-in code drifts from the spec.
2. **`build.rs` via the `spargen` library API.** For teams that want spec→code coupling in one build.

Output is **deterministic**: same `spargen` version + same spec + same config ⇒ byte-identical output, `rustfmt`-clean (via `prettyplease`), and passes `cargo clippy -D warnings`. There is deliberately **no proc-macro mode** (§3.2).

### 2.3 Internal architecture: subsystems and dependency plan

One published crate does not mean one blob. The crate is internally partitioned into subsystems (top-level modules) with a **declared, machine-enforced dependency DAG** — module privacy alone cannot enforce layering inside a single crate, so an `xtask lint-layers` CI job parses inter-module `use` edges and fails on any edge not in the table below.

Design rules the partition follows:

1. **Version-specific frontends, version-agnostic core.** Everything that knows OpenAPI 3.1 syntax lives in one frontend subsystem (`oas31`), which lowers into a spec-version-agnostic intermediate representation. Adding a future spec version means adding a sibling frontend (`oas32`) and touching nothing downstream: codegen never sees a spec document; frontends never see Rust tokens. **The IR is the coupling firewall and the primary extension seam.**
2. **Diagnostics are a subsystem, not a byproduct.** `diag` owns codes, severities, the pointer+span model, the `INT-###` interpretation registry, and rendering — and it holds the S/W/R disposition table and error index **as data**, from which both the published docs and frontend behavior derive. One source of truth; an exhaustiveness test fails the build if code and docs diverge.
3. **The emitted runtime is real source, not string templates.** `support` is a normal Rust source tree that spargen compiles and tests directly in its own suite, then embeds verbatim (`include_str!`) into output. The runtime is never validated only through generated artifacts.
4. **No plugin architecture.** Frontends and backends are compiled-in modules; there is no trait-object registry or dynamic dispatch seam. Abstraction cost is paid when a second implementation exists, not before (consistent with §3.2.11).

| Subsystem (module) | Owns (domain problem) | May depend on |
|---|---|---|
| `diag` | Diagnostic codes/severities, JSON Pointer + span model, `INT-###` registry, human/JSON renderers, S/W/R disposition table as data | — |
| `source` | Input bundles (JSON/YAML, relative-file `$ref` loading), span-preserving event-based parse into a `SpannedValue` tree | `diag` |
| `ir` | Version-agnostic API model: operation set, type graph, auth requirements, media map; provenance (pointer + span) on every node; well-formedness invariants | `diag` |
| `oas31` | OAS 3.1.1 typed document model, structural/meta-schema validation, `$ref` resolution, per-keyword disposition audit, lowering `SpannedValue` → IR | `source`, `ir`, `diag` |
| `name` | Deterministic identifier allocation: casing, keyword escaping, collision resolution, `operationId` synthesis (D9) | `ir`, `diag` |
| `support` | The runtime support source shipped inside generated output: dispatch routines, FR5 error taxonomy, `ResponseValue<T>` | — (compiles standalone against reqwest/serde only) |
| `codegen` | IR + allocated names → Rust tokens: models, client, embedded `support`; deterministic item ordering; `prettyplease` formatting | `ir`, `name`, `support`, `diag` |
| `emit` | Output layout (module vs standalone crate), `Cargo.toml` synthesis, provenance header, `--check` diffing | `codegen`, `diag` |
| `cli` (feature `cli`) | Command surface (`generate`/`check`/`explain`), exit-code contract, `--format json` | facade |
| facade (`lib.rs`) | Public `build.rs` API: config types and thin pipeline orchestration | all of the above |

Pipeline: `source` → `oas31` → (`ir` + `name`) → `codegen` → `emit`, with `diag` as the only vocabulary shared across stages.

Pre-planned extension seams:

- **New spec version:** sibling frontend selected by the document's `openapi` field, lowering to the shared IR. If a new version requires IR growth, changes are additive and every frontend must keep satisfying IR invariants — verified by the IR snapshot suite (§7.5), which pins the frontend/backend contract.
- **New output flavor** (hypothetical): sibling backend consuming IR; frontends untouched.
- **New matrix rows / diagnostics:** data change in `diag` plus a fixture; behavior follows from data.


---

## 3. Scope

### 3.1 Strictly in scope

- OpenAPI documents declaring `openapi: 3.1.x` (any patch), in JSON or YAML, including `$ref`s to **local relative files** within the input bundle.
- JSON Schema 2020-12 under the default OAS 3.1 dialect, per the support matrix in §5.2 — every keyword has a defined disposition; none is silently mishandled.
- Async client on `reqwest`, one concrete HTTP stack (§6.1).
- Media types: `application/json` (canonical), `application/x-www-form-urlencoded`, `application/octet-stream` (bytes in; bytes or stream out), `text/plain`.
- Auth plumbing for `http` (`bearer`, `basic`) and `apiKey` (header/query/cookie) schemes; caller-supplied tokens for `oauth2`/`openIdConnect` (§5.5).
- Typed success and error responses per operation, including `default` and status-range (`2XX`) responses; response status and headers exposed via `ResponseValue<T>`.
- The validation corpus, harness, and conformance loop of §7.

### 3.2 Definitely excluded (with justification)

1. **OpenAPI 3.0.x, 2.0, and automatic 3.0→3.1 up/down-conversion.** The dialects differ semantically (`nullable`, `exclusiveMinimum`, file-schema keywords); silent conversion is precisely the class of quiet miscompilation this project exists to eliminate. A 3.0 input is rejected with a dedicated diagnostic explaining this and naming alternatives.
2. **Server-side or any non-client code generation.** Different product; the server side of the Rust ecosystem is already healthy.
3. **A proc-macro interface (`generate_api!`-style).** Macros are the root cause of the vague-error experience: diagnostics land in invisible expanded code, IDE support degrades, and the generator becomes a compile-time dependency of every consumer. This is a debuggability requirement, not a taste choice.
4. **Runtime/dynamic clients** (interpreting the spec at runtime). Defeats the compile-time premise and the binary-size goal simultaneously.
5. **Runtime validation of non-shape schema constraints** (`pattern`, `minimum`, `maxLength`, …). The generated types are the contract; enforcing validation keywords at runtime buys little for clients and costs size and latency. These keywords are ignored *with a warning*, never silently (§5.2).
6. **Built-in retries, backoff, caching, or middleware.** Callers inject their own configured `reqwest::Client` (with `tower`/`reqwest-middleware` if they wish). Spargen's contribution is an error taxonomy with an `is_transient()` classifier so caller-side retry policies are trivial to write (§5.7). Keeps the runtime crate near-zero-dependency.
7. **OAuth2/OIDC token acquisition flows.** Interactive flows, token storage, and refresh policy are application concerns; Spargen accepts a token provider and attaches credentials correctly, nothing more.
8. **XML bodies** (niche in the 3.1 ecosystem; heavy dependency) and **`multipart/form-data`** (drags in the mime ecosystem; bounded out until the corpus demonstrates demand). Both are R-class: generation *fails with a precise diagnostic*, so the exclusion is loud, not silent.
9. **Codegen for `webhooks`, `callbacks`, and `links`.** These describe server-initiated or hypermedia flows a generated client cannot enact. Parsed and acknowledged with a warning; no code emitted.
10. **Network-fetched `$ref`s at generation time.** Builds must be hermetic and deterministic; remote documents are vendored into the corpus/input bundle via tooling instead.
11. **Custom templates or codegen plugin hooks.** Template systems are how other generators arrived at unmaintainable output and unownable error messages. One opinionated output, fully tested.
12. **A blocking client.** `reqwest::blocking` embeds a runtime anyway; callers can `block_on`. Excluding it halves the test and size matrix.
13. **Spec authoring, editing, or general-purpose linting.** `spargen check` exists solely as a generation support-audit.
14. **A roadmap.** Scope is defined by this document's in/out lists and the corpus, not by dated promises. The only forward-looking artifact is §10.1, whose items are explicitly non-binding.

### 3.3 Normative sources (what "OpenAPI 3.1.x" strictly means)

"We implement OpenAPI 3.1" is exactly the loose claim that produced divergent generators. Spargen conforms to the following specific documents, applied in this precedence order wherever they conflict or are ambiguous:

| Prec. | Document | Pinned version / URL | Status | Governs |
|---|---|---|---|---|
| 1 | OpenAPI Specification | **v3.1.1** — `https://spec.openapis.org/oas/v3.1.1.html` | Normative | Primary text. Documents declaring any `openapi: 3.1.*` are accepted and interpreted per 3.1.1 (3.1.1 is a clarification-only patch over 3.1.0; per OAI versioning policy, patch releases do not change semantics) |
| 2 | OAS 3.1 base dialect & meta-schemas | `https://spec.openapis.org/oas/3.1/dialect/base` and `https://spec.openapis.org/oas/3.1/meta/base` | Normative | The default `jsonSchemaDialect` (FR1), including the OAS keywords `discriminator`, `xml`, `example`, `externalDocs`; drives structural validation in `spargen check` |
| 3 | JSON Schema Core 2020-12 | `draft-bhutton-json-schema-01` — `https://json-schema.org/draft/2020-12/json-schema-core` | Normative | `$ref`/`$defs`/`$anchor`, applicator and evaluation semantics |
| 4 | JSON Schema Validation 2020-12 | `draft-bhutton-json-schema-validation-01` — `https://json-schema.org/draft/2020-12/json-schema-validation` | Normative | Per-keyword semantics behind the S/W/R matrix; `format` handled per the **format-annotation vocabulary** (annotation, not assertion) — the formal basis for §6.2's opt-in type mappings |
| 5 | RFC 8259 (JSON); YAML 1.2.2 (`https://yaml.org/spec/1.2.2/`) | as cited by OAS 3.1.1 | Normative | Input documents; YAML restricted to the JSON-compatible subset OAS prescribes |
| 6 | RFC 6901 | JSON Pointer | Normative | Diagnostic addressing (FR6) and `$ref` fragment resolution |
| 7 | RFC 3986 | URI | Normative | `$ref` resolution, server URL handling |
| 8 | RFC 6570 | URI Template | Informative | The definitional basis of parameter `style`/`explode` behavior in §5.2 |
| 9 | RFC 9110 | HTTP Semantics | Informative | Status-code classification in FR5; wire behavior is otherwise delegated to reqwest |

Conformance mechanics:

- The exact texts and meta-schemas above are **vendored under `spec/`** in-repo — pinned and checksummed like the corpus — so conformance targets a fixed artifact, never a moving URL.
- Where these sources are genuinely ambiguous or in tension, the chosen reading is recorded as a numbered **interpretation** (`INT-###`) in the published documentation and linked from the support matrix and from any diagnostic whose behavior depends on it. Ambiguity is documented, never silent.
- Future 3.1.x patch releases (3.1.2+) are accepted immediately by version match and adopted for interpretation via a vendored-spec bump PR — a reviewed diff, not an ambient change.
- Everything else at `spec.openapis.org` (extension registries, non-3.1 documents) is **non-normative** for Spargen.

---

## 4. Users and use cases

- **Rust service teams consuming internal 3.1 APIs** — the spec is emitted by a sibling utoipa/aide service; the client must track it exactly, with CI drift detection.
- **SDK publishers** — generate a standalone, publishable crate for a public API without vendoring a generator into consumers' builds.
- **Cross-compiled and embedded-adjacent libraries** (static musl binaries, mobile FFI layers, edge workers) — the motivating binary-size case: the client must add kilobytes, not megabytes, over an app that already links reqwest and serde.
- **CI/platform engineers** — machine-readable `spargen check` output as a contract gate between spec producers and client consumers.

---

## 5. Functional requirements

### FR1 — Input handling and versioning

- Accept a single JSON or YAML document plus local relative-file `$ref`s. Reject absolute-URL `$ref`s (R-class, §3.2.10).
- `openapi` field MUST match `3.1.*`, interpreted per the pinned sources of §3.3. Anything else is rejected with a version-specific diagnostic; the 3.0 rejection message explains the dialect difference and does not offer conversion.
- `jsonSchemaDialect`, if present, must be the default OAS 3.1 dialect (`https://spec.openapis.org/oas/3.1/dialect/base`, §3.3); other dialects are rejected.
- Parsing MUST be span-preserving (file, line, column retained per node) to power FR6. Decided: an in-house `SpannedValue` tree built by event-level parsing, owned by the `source` subsystem (D4).

### FR2 — Spec feature policy: every keyword has a disposition

Every OpenAPI/JSON-Schema construct is classified into exactly one of three classes. **There is no fourth, undefined behavior.** The published support matrix is the product's central debuggability artifact.

| Class | Meaning | Behavior |
|---|---|---|
| **S — Supported** | Faithfully represented in generated types/behavior | Code generated |
| **W — Ignored, warned** | Affects runtime *validation* but not the static *shape* of data | Generation proceeds; warning with code + JSON Pointer, once per site |
| **R — Rejected** | Affects data shape or wire behavior in a way Spargen does not represent | Generation fails; error with code + pointer + span + remedy |

Invariant: generated code never silently degrades a typed schema to `serde_json::Value`. `Value` appears only where the spec itself is untyped (`{}` / `true` schemas), which is faithful, not lossy.

**Support matrix (normative summary; the full per-keyword table ships as documentation and is snapshot-tested):**

| Area | S | W | R |
|---|---|---|---|
| Document | `info`, `servers` (+ variable substitution), `paths`, `components`, `tags`, `security` | `externalDocs`, `webhooks`/`callbacks`/`links` (no codegen), `x-*` extensions | — |
| Parameters | `path`/`query`/`header`/`cookie`; styles `simple` and `form` with standard `explode` defaults; `content`-typed (JSON-serialized) params; `required`, `deprecated` → `#[deprecated]` | `example(s)` | `deepObject`, `spaceDelimited`, `pipeDelimited`, `allowReserved`, `allowEmptyValue` |
| Bodies | Media types listed in §3.1; `required` | — | XML, multipart, other media types |
| Responses | Per-status typed bodies; `default`; status ranges (`2XX` etc.); multiple success statuses → success enum; response headers via `ResponseValue` | response `links` | — |
| Schema: shape | `type` (incl. type arrays / `"null"`), `enum`/`const` over homogeneous scalars (D6), `properties`/`required`, `additionalProperties` (bool and schema → map or `deny_unknown_fields`), `items`, `prefixItems`, `allOf` (object merge), `oneOf` + `discriminator` → tagged enum, `$ref` (incl. cycles → `Box`), `$defs`, `format` mappings (§6.2), `contentEncoding: base64` → bytes, `default` (deserialization defaults), `deprecated`, `title`/`description` → rustdoc | `readOnly`/`writeOnly` (annotation only, D2) | `patternProperties`, `$dynamicRef`/`$dynamicAnchor`, non-object `allOf` merges, heterogeneous or structured `enum` value sets (D6) |
| Schema: validation-only | — | `pattern`, `minimum`/`maximum`/`exclusive*`, `multipleOf`, `min/maxLength`, `min/maxItems`, `uniqueItems`, `min/maxProperties`, `if`/`then`/`else`, `not`, `unevaluated*`, `propertyNames`, `dependentSchemas`/`dependentRequired` | — |
| Composition w/o discriminator | Statically provably disjoint variant sets → enum with a generated order-independent deserializer (D1) | — | Non-disjoint variant sets: rejected, naming the overlapping variants and suggesting a `discriminator` (D1). serde `untagged` is never emitted |
| Security | `http: bearer/basic`, `apiKey` (header/query/cookie) | `oauth2`/`openIdConnect` scheme *metadata* (flows not implemented; token via provider) | — |

### FR3 — Shape of generated code

- One `types` (models) module + one `Client`. `Client::new(base_url)` and `Client::with_client(reqwest::Client, base_url)` (the BYO-client injection point for TLS choice, proxies, middleware, timeouts).
- One method per operation, named from `operationId` (deterministic scheme per D9, owned by `name`). Required parameters are positional; optional parameters travel in a per-operation `…Params` struct deriving `Default` (D3).
- Return type: `Result<ResponseValue<T>, Error<E>>` where `ResponseValue<T>` exposes status, headers, and `into_inner()`; `E` is the operation's typed error body (enum when multiple error statuses are documented).
- Spec `description`/`summary` become rustdoc, so IDE hover shows API docs.
- `#![forbid(unsafe_code)]` across all generated code, including the emitted `support` module.
- Deterministic ordering of items regardless of input map ordering (stable diffs for checked-in code).

### FR4 — Authentication

- Client construction accepts per-scheme credentials: static secrets or an async token-provider callback (`Fn() -> Future<Output = Result<SecretString, _>>`) for rotating tokens.
- Operation-level `security` requirements determine which credentials attach where (header/query/cookie); missing required credentials are a *construction-time* error, not a 401 surprise.
- Secrets are `Debug`-redacted throughout.

### FR5 — Runtime fault tolerance: error taxonomy

The generated client never panics on any input from the network. Every failure maps to one variant of a closed taxonomy defined in the emitted `support` module (§2.1) — identical in shape across all Spargen output and versioned with the generator:

| # | Class | Examples / contents |
|---|---|---|
| 1 | **Request construction** | Invalid base URL; parameter/body serialization failure (typed, near-impossible by construction) |
| 2 | **Transport** | DNS resolution failure; connection refused/reset; TLS handshake or certificate errors |
| 3 | **Timeout** | Connect vs total-request timeouts distinguished (as configured on the injected `reqwest::Client`) |
| 4 | **Protocol** | Malformed HTTP, decompression failure |
| 5 | **Redirect** | Redirect-policy exhaustion (per injected client's policy) |
| 6 | **Documented API error** | Non-success status documented in the spec → parsed into the operation's typed `E` |
| 7 | **Undocumented status** | `UnexpectedStatus { status, headers, body: Bytes }` — raw body preserved for forensics |
| 8 | **Decode** | Response body fails to deserialize → serde error *path* plus retained raw body (64 KiB default retention cap, configurable at client construction; D7) |
| 9 | **Interrupted body** | Connection dropped mid-stream on streamed responses |
| 10 | **Cancellation** | Dropping the future is safe and side-effect-free beyond standard HTTP semantics (documented guarantee) |

`Error::is_transient()` classifies transport failures, timeouts, 429, and 5xx as retry-worthy so callers can wrap any retry policy around the client without Spargen shipping one (§3.2.6). All variants implement `std::error::Error` with full source chains.

### FR6 — Generation-time diagnostics (debuggability)

- Every diagnostic carries: severity, a **stable code** (`E###`/`W###`), the **JSON Pointer** to the offending construct, **file:line:column**, a one-line explanation, and a remedy suggestion. Rendered rustc-style for humans; `--format json` for CI.
- **Batch reporting**: generation collects all errors (capped) rather than stopping at the first.
- `spargen check spec.yaml` — support-audit without codegen; exit code suitable as a CI contract gate between spec producers and consumers.
- `spargen explain E042` — extended documentation per code, mirrored on a published errors-index page alongside the full support matrix.
- Diagnostic wording is **snapshot-tested** (§7.6): error messages are product surface, and changing one is a reviewed diff.

---

## 6. Non-functional requirements

### NFR1 — Performance and HTTP stack

- **`reqwest`, and only `reqwest`.** Rationale: it is the dependency most Rust applications already carry (maximizing dependency sharing, §6.2), exposes the programmatic control needed (custom client injection, TLS backend passthrough, streaming, HTTP/2 pooling), and covers wasm. A generic HTTP-backend abstraction is explicitly rejected: trait-generic clients multiply monomorphization (binary size) and produce exactly the opaque generic compile errors this project opposes. One concrete stack, fully tested.
- Async-only; native targets ride reqwest's Tokio backend.
- No runtime spec interpretation, no regex-based URL construction; paths compile to static segment concatenation. Request hot path performs no allocations beyond those inherent to URL/body construction.

### NFR2 — Binary size

Strategy 1 — *minimal boilerplate, maximum sharing*:
- Per-operation generated functions are thin `#[inline]` shims over a small set of **non-generic** dispatch routines in the emitted `support` module (build URL → attach auth → send → classify status). Sharing happens *within* a generated client rather than via a shared crate (§2.1); monomorphization occurs only where unavoidable: once per distinct body type for encode/decode.
- Runtime dependencies of generated code: `reqwest` (no default features), `serde`, `serde_json`, `bytes` — nothing else, and **no Spargen crates** (§2.1). `syn`/`quote`/`prettyplease` never appear anywhere near a consumer's runtime graph.

Strategy 2 — *only dependencies most apps already have*:
- TLS backend is the consumer's choice via reqwest feature passthrough (`rustls` recommended for cross-compilation; never forced).
- `format` type mappings are feature-gated: `uuid` and `time` features (default **on**) map `format: uuid`/`date-time`/`date` to the respective crates; disabling a feature falls back to plain `String` (a documented, deliberate loss of typing for size-critical builds).

Enforcement:
- A reference application (50-operation corpus client over a bare reqwest+serde baseline) is size-tracked in CI via `cargo-bloat`/`twiggy`. Initial budget to be baselined at first implementation; thereafter a **>5% regression fails CI**. The budget number is a tracked artifact, not folklore.

### NFR3 — Compile time

Generated code avoids compile-time-heavy patterns (no proc-macros in output beyond `serde::Derive`, no deep generic towers). Cold-build time of the reference client is tracked in the same CI job as size.

### NFR4 — Targets (cross-compilation)

| Tier | Targets | Guarantee |
|---|---|---|
| A (CI-built and tested) | `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc` | Green or release blocked |
| B (best effort) | `wasm32-unknown-unknown` (reqwest wasm backend), `aarch64-linux-android`, `armv7-unknown-linux-musleabihf` | Build-checked; issues accepted |

`no_std` is out of scope (reqwest/serde_json require std).

### NFR5 — Stability policy

- MSRV policy: latest stable minus 4 releases; bumping MSRV is a minor version, documented.
- Generated-output stability: identical (`spargen` version, config, spec) ⇒ byte-identical output. Output-affecting changes are changelogged; `generate --check` makes drift visible in consumer CI. What constitutes a breaking generator change is defined in D12: the semver surface is the public API of generated output.

---

## 7. Development methodology and validation

### 7.1 Project-wide methodology

- **Disposition-driven development.** No behavior change lands without first changing its source of truth in `diag` — a matrix row, a diagnostic code, or an `INT-###` interpretation — together with a fixture that exercises it. Implementation follows the failing fixture. Bug reports follow the same law: every bug becomes a fixture or corpus entry *before* its fix, making regressions structurally impossible to reintroduce silently.
- **Everything reviewable is a diff.** Generated-code snapshots, IR snapshots, diagnostic wording, vendored specs (§3.3), corpus checksums, and size numbers all live in-repo, so every behavioral delta surfaces in code review. CI is the arbiter of done.
- **Trunk-based, gate-heavy.** Small PRs to main behind the full gate set (layer lint, snapshot suites, corpus harness, conformance loop, size/compile budgets, `cargo-deny`). The PR template requires declaring: subsystems touched, matrix delta, diagnostics delta, output-API delta (D12).
- **Determinism as a standing invariant.** Double-generation byte-equality runs on every PR, not only at release.
- **Docs generated from data.** The errors index and support matrix render from `diag`'s tables; drift between documentation and behavior is impossible for those artifacts by construction.
- **Releases are mechanical.** The release checklist is the Definition of Done (§8) plus the D12 output-API diff, which dictates the version bump; the changelog's generated-output section is derived from that diff.

### 7.2 Corpus: naming and layout

Proposal: use **`corpus/`** for the Git-LFS-managed real-world specs (a *corpus* is representative real data; *fixtures* connote minimal synthetic inputs) and keep `tests/fixtures/` for small hand-written unit specs that stay out of LFS and in the fast test path.

```
corpus/
  github-rest/
    spec.json          # LFS
    meta.toml
  utoipa-petstore/
    spec.yaml          # LFS
    meta.toml
  ...
```

`meta.toml` schema:

```toml
name           = "GitHub REST API (3.1 description)"
source_url     = "https://…"
retrieved      = 2026-07-03
upstream_ref   = "descriptions-next @ <commit>"
sha256         = "…"                # CI-verified; tamper/drift detection
license        = "MIT"              # specs are redistributed in-repo
expect         = "generate"         # or "reject"
expected_diagnostics = []           # for expect = "reject": ["E014@/components/schemas/Foo/patternProperties"]
tags           = ["utoipa", "large"]
notes          = ""
```

Selection criteria for corpus entries: declares `openapi: 3.1.*`; a real production surface (not a toy); collectively diverse across the §5.2 matrix. Seed set: GitHub's 3.1 REST description, output of utoipa 5.x / aide / poem-openapi reference apps, MOTIS, plus project-selected APIs. Refresh via `cargo xtask corpus refresh <slug>` (re-fetch, update checksum/date); CI verifies checksums so LFS content can't drift silently.

### 7.3 Auto-generated corpus tests

The harness discovers every `meta.toml` and generates one test per entry, mechanically:

1. Parse + validate; assert disposition matches `expect`.
2. `expect = "generate"`: emit code into a hermetic temp crate (vendored lockfile) and require `cargo check` **and** `cargo clippy -D warnings` to pass; additionally assert the resolved dependency graph contains **no Spargen crates** (freestanding-output gate, §2.1).
3. `expect = "reject"`: assert the exact `expected_diagnostics` fire (code @ pointer), snapshot the rendered messages.
4. Determinism: generate twice, assert byte equality.
5. Golden snapshots (insta) of generated output for a designated subset, so codegen changes are reviewed as diffs.

### 7.4 End-to-end conformance loop

An in-repo reference service (axum + utoipa) closes the loop: its *emitted* spec is fed to Spargen, and integration tests drive the generated client against the live service over real HTTP. The service includes a **misbehaving mode** — wrong content-type, truncated bodies, undocumented statuses, artificial latency, connection resets — asserting that every injected fault maps to the documented FR5 variant. This is the fault-tolerance test bed and simultaneously the utoipa-compatibility guarantee.

### 7.5 Per-subsystem validation

Each subsystem of §2.3 has a named validation process; a subsystem without its suite fails the Definition of Done (§8).

| Subsystem | Validation process |
|---|---|
| `diag` | Exhaustiveness tests: every code has `explain` text, a docs entry, and ≥1 fixture that triggers it; render snapshots for both human and JSON formats |
| `source` | Span-accuracy fixtures (asserted line/col for known constructs); `cargo-fuzz` targets on the JSON/YAML event parsers (generation-time fault tolerance); malformed-input fixture set (truncation, invalid UTF-8, duplicate keys, depth bombs) |
| `oas31` | One fixture per matrix row per disposition — S, W, and R each demonstrably exercised; validation against the vendored OAS meta-schemas (§3.3); reject-fixtures assert exact `code@pointer`; **an IR snapshot per fixture**, testing the frontend at the IR seam independent of codegen |
| `ir` | Invariant checker runs after every lowering in tests and debug builds; property tests that passes preserve well-formedness; a stable textual IR dump format underpins the snapshot suite |
| `name` | Property tests: determinism, injectivity within scope, output is always a valid Rust identifier (keywords, Unicode, leading digits); exhaustive collision fixtures |
| `support` | Compiled and unit-tested directly as an in-repo test target (never only via generated output); the conformance loop's misbehaving-server mode (§7.4) exercises every FR5 taxonomy variant; standalone `clippy -D warnings` and `forbid(unsafe_code)` |
| `codegen` | Golden Rust snapshots per fixture; determinism double-run; compile gates (`cargo check` + clippy) via the corpus harness; output-API diff (D12) |
| `emit` | `--check` contract tests (clean / drifted / missing); standalone-crate packaging validated with `cargo publish --dry-run`; the freestanding dependency-tree gate (§2.1) |
| `cli` | Black-box exit-code contract tests; `--format json` schema tests; help-text snapshots |
| cross-cutting | Corpus harness (§7.3), conformance loop (§7.4), size and compile-time budgets (NFR2/NFR3), layer lint (§2.3) |

### 7.6 Additional cross-cutting gates

- **Diagnostics snapshots**: every error/warning message under test; wording changes are reviewed diffs.
- **Size and compile-time CI** per NFR2/NFR3.
- **Supply chain**: `cargo-deny` (licenses, advisories, duplicate deps) on every PR.
- **Layering**: the §2.3 module-dependency lint (`xtask lint-layers`) on every PR.

---

## 8. Definition of done (acceptance criteria — not a roadmap)

1. 100% of `expect = "generate"` corpus entries produce deterministic code passing check, clippy, and snapshots.
2. Every OpenAPI 3.1 / JSON Schema 2020-12 keyword appears in the published matrix with an S/W/R disposition — zero undefined behaviors.
3. Conformance loop green, including every fault-injection case mapping to its documented error variant.
4. 100% of diagnostics carry code + pointer + span; errors index published; `explain` implemented.
5. Size budget baselined and enforced; all Tier-A targets green in CI.
6. Quickstart requires ≤10 lines of consumer code from spec to first typed API call.
7. Freestanding-output gate holds for every corpus client: no Spargen crates in any resolved runtime dependency tree; runtime deps are exactly the §2.1 set.
8. The §2.3 layer lint is green, and every subsystem ships its §7.5 validation suite, passing.

---

## 9. Design decisions (all former open questions resolved)

Every previously open question is now either **decided** (with brief justification) or **explicitly deferred** to §10.1, where nothing is committed.

1. **Undiscriminated `oneOf`/`anyOf` — decided.** Generate an enum with a *generated, order-independent deserializer* only when variants are statically provably disjoint (disjoint `type`s, distinguishing `const`/literal fields, or mutually exclusive required-property sets); otherwise reject, naming the overlapping variant pair and suggesting a `discriminator`. serde `untagged` is never emitted — first-match-wins deserialization can silently misparse, which is incompatible with the correctness guarantee. `anyOf` follows the same rule: its "may match several" semantics coincides with `oneOf` exactly in the disjoint case, the only case accepted.
2. **`readOnly`/`writeOnly` — decided.** One model type per schema; the keywords are W-class annotations surfaced in the field's rustdoc. Split request/response types roughly double model code, directly against the binary-size requirement, for a gain the corpus does not yet evidence. Split-type generation: deferred (§10.1).
3. **Operation signatures — decided.** Required parameters positional; optional parameters in a per-operation `…Params` struct deriving `Default` (ergonomic via struct-update syntax). Builders rejected: they roughly triple per-operation generated code to provide what a plain struct with `Default` already does.
4. **Span-preserving parsing — decided.** `source` owns an in-house `SpannedValue` tree built from event-level JSON/YAML parsing. The serde-derive path is rejected because serde discards spans and FR6 diagnostics are non-negotiable; the component is small, foundational, and worth owning outright.
5. **Numerics — decided.** `int32`→`i32`, `int64`→`i64`, unformatted `integer`→`i64`, `number`→`f64`. Out-of-range wire values surface as Decode errors (FR5 #8), never silent wraps; `serde_json/arbitrary_precision` is not used (cost without corpus-demonstrated need). Arbitrary-precision integers: deferred (§10.1).
6. **Non-string `enum`/`const` — decided.** Homogeneous scalar value sets (all-string, all-integer, all-boolean) generate unit-variant enums with explicit serialization; heterogeneous or structured value sets are R-rejected. This covers the real cases (integer status enums) without inventing representations for pathological ones.
7. **Raw-body retention — decided.** Error variants retain at most 64 KiB of response body by default, configurable at client construction; truncation is flagged on the error; fully streamed bodies retain nothing after handoff. Bounds memory under adversarial servers (a fault-tolerance requirement) while preserving forensics.
8. **`default` keyword — decided.** Deserialization-only: missing fields with scalar schema defaults are filled via serde defaults; requests are never auto-populated — the server owns defaulting, and implicit request mutation is surprising. Non-scalar defaults are W.
9. **Identifier scheme — decided.** Owned by `name`: Rust-conventional casing via Unicode-XID-aware segmentation; keywords escaped as raw identifiers (`r#type`) where legal, trailing underscore otherwise; in-scope collisions resolved with a stable disambiguator derived from the item's JSON Pointer (order-independent, hence deterministic under spec reordering); a missing `operationId` is synthesized from method + path template. Property-tested per §7.5.
10. **wasm32 — decided as-is.** Remains Tier B. Promotion criteria (a motivated consumer plus CI runner budget): deferred (§10.1).
11. **Success-response shape — decided.** Per-status precedence: exact code > range (`2XX`) > `default`. A single success source yields plain `T`; multiple sources yield a per-operation success enum; `default` contributes to the error type unless it is the operation's only success source.
12. **Generator semver — decided.** The semver surface is the **public API of generated output**. Changes altering generated signatures, type shapes, or variant sets ⇒ major; output changes invisible to that API (formatting, rustdoc, `support` internals) ⇒ minor; generator-internal fixes ⇒ patch. Enforced mechanically: CI diffs the public API of corpus-generated clients against the previous release (cargo-public-api-style), and the diff dictates the bump (§7.1).
13. **OpenAPI 3.2 — resolved architecturally.** The frontend seam (§2.3) is the answer: a sibling `oas32` module lowering to the shared IR, all downstream subsystems untouched. Whether and when to build it: deferred (§10.1).
14. **Shared-runtime mode — decided against, for now.** The freestanding model (§2.1) stands. Size CI includes a synthetic multi-client binary so the duplication cost is a measured number rather than a fear; any revisit is deferred (§10.1) and argues from that number.

---

## 10. Long-term: research agenda toward de facto status

Not a roadmap — a list of the things to research and prove, because they are what made incumbents defaults elsewhere:

- **Coverage credibility.** Publish the corpus pass-rate as a live dashboard/badge; make "file your failing spec, it becomes a corpus entry" the primary issue template. The corpus *is* the marketing.
- **Own the Rust-server loop.** First-class, CI-proven compatibility with utoipa, aide, poem-openapi, and Dropshot-emitted 3.1 output; contribute conformance fixes upstream. If every Rust server framework's tutorial can say "and generate your client with Spargen," the default is won.
- **Migration path.** A Progenitor/openapi-generator migration guide, with `spargen check` reporting exactly which constructs in an existing spec change disposition.
- **Anchor adopters.** Two or three visible projects shipping Spargen-generated crates; a standalone published SDK is the strongest proof.
- **Trust surface.** `forbid(unsafe_code)`, `cargo-deny`, near-zero runtime dependency count, MSRV and semver discipline, and *published* size/compile-time benchmarks against alternatives.
- **Diagnostics as brand.** The support matrix and errors index, publicly documented — "we tell you exactly what happens to every keyword" is the differentiator no template-based generator can copy cheaply.
- **Watch 3.2** per D13 and §10.1: the frontend seam means readiness is an architectural fact, not a promise.

### 10.1 Deferred options (explicitly non-binding)

Nothing below is committed, scheduled, or promised. Each item activates only on corpus or user evidence, and only by amending this document first:

- Split request/response model types for `readOnly`/`writeOnly` (D2) — trigger: corpus measurement showing acceptable size cost against demonstrated correctness need.
- Arbitrary-precision integers (D5) — trigger: real 3.1 corpus specs that require them.
- `multipart/form-data` bodies (§3.2.8) — same trigger discipline.
- Opt-in shared-runtime packaging for multi-client binaries (D14) — trigger: the size-CI duplication number becoming material.
- wasm32 promotion to Tier A (D10) — trigger: a motivated consumer plus CI budget.
- `oas32` frontend (D13) — trigger: sustained submission of real OpenAPI 3.2 documents to the corpus intake.
- Additional output flavors via the backend seam (§2.3) — no candidate identified; listed only because the seam exists.s