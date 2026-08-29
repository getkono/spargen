# spargen

A compile-time-correct Rust client generator for OpenAPI 3.1.x and 3.2.x. The [`README.md`](README.md)
carries the product contract; [`docs/support-matrix.md`](docs/support-matrix.md) and
[`docs/errors.md`](docs/errors.md) are the operational surface — read them before non-trivial
changes.

## Workspace

- `spargen/` — the primary published crate (library + `cli`-gated binary). Internally partitioned
  into subsystems with a declared dependency DAG: `diag`, `source`, `ir`, `oas31`, `name`,
  `support`, `codegen`, `emit`, `compat`, `surface`, `cli`, and the `lib.rs` facade
  (`cache`, `config`, and `runtime_contract` are facade plumbing, not subsystems). Every subsystem
  `mod.rs` declares its allowed dependencies in a `//! layer-deps:` header — keep those honest.
- `spargen-macro/` — the second published crate: a thin `proc-macro` shim exposing
  `generate_api!`, a shim over spargen's private in-memory renderer. It depends on `spargen` (host-only); `spargen`
  must **never** depend back on it (that would cycle). A proc-macro crate and everything it reaches
  are host/build-time only, so neither crate enters a consumer's runtime graph — the invariant
  below is unchanged. `examples/petstore-macro` is its end-to-end guard.
- `support-runtime/` — the freestanding runtime embedded verbatim into generated output.
  `publish = false`; its unconditional dependencies are exactly `reqwest` / `serde` / `serde_json` /
  `bytes` / `secrecy` / `futures-core` (the stream module and its dependency are emitted only for
  APIs with sequential responses), plus three optional ones behind features for the conditionally
  embedded modules: `quick-xml` (`xml`), `tokio` (`blocking`), and `time` (`time`, the RFC 3339
  date newtypes). No spargen crate may ever appear in a consumer's runtime graph. Each source file
  keeps its `#[cfg(test)]` module last — everything above that marker is embedded into generated
  code and must compile there.
- `examples/` — each its own workspace, so a consumer's crate layout is what is actually tested.
  `petstore/` (build.rs path) and `petstore-macro/` (macro path) are driven over real HTTP by
  `mise run example`; `github-api/` generates the pinned 12.9 MB GitHub 3.1 description and is
  compile-checked natively and for wasm by `mise run github-api`. All three must stay green.
- `corpus/` — pinned real-world descriptions with expected outcomes in `corpus/manifest.toml`
  (`expect = "generate"` / `"reject:E###"`), mirrored in `corpus/README.md`. Files are Git-LFS.
  `corpus/recipes/` holds the hand-written framework-output specs `tests/recipes.rs` drives.
- `fuzz/` — a cargo-fuzz crate (libFuzzer, nightly, **excluded from the workspace**), manual-only:
  `cargo +nightly fuzz run frontend`. Its always-on counterpart is `tests/fuzz_frontend.rs`, a
  fixed-seed proptest no-panic harness.
- `spargen/benches/` — criterion benchmarks over the generation pipeline (`mise run bench`). CI
  records them on tags as an artifact; they are **not** a gate.
- `docs/book/` — the mdBook site (`mise run docs`), which includes the standalone `docs/*.md`
  rather than duplicating them. `references/` carries the vendored OpenAPI specification texts —
  `3.2.0.md` is the ground truth for any 3.2 conformance question.
- `deny.toml` — the supply-chain policy (`mise run deny`).

## Quality

Validate changes:

```bash
mise run check      # cargo check --workspace --all-features
mise run fmt        # cargo fmt --all
mise run lint       # cargo clippy --workspace --all-targets --all-features -- -D warnings
mise run test       # cargo test --workspace --all-features
mise run powerset   # cargo hack: every feature combination, not just --all-features
mise run corpus-smoke  # pinned real-world specs
mise run example    # both petstore examples over a local mock server
mise run github-api # the full GitHub client: native strict clippy + wasm32
mise run deny       # supply-chain audit
mise run docs       # build the mdBook site (fails on broken links/includes)
```

`hk.pkl` wires four of these into git hooks through the same mise tasks CI runs: `fmt` and `lint`
on pre-commit, `test` and the `convco` commit-message check on pre-push. The rest — `check`,
`powerset`, `corpus-smoke`, `example`, `github-api`, `deny`, `docs` — run only in CI; they are too
slow for a hook, so a green pre-push is not a green CI. Run `mise run hooks` once to install them.

CI additionally gates five things the list above does not name: `msrv` (the declared
`rust-version` floor still compiles), `package` (`cargo publish --dry-run`, which is what keeps the
runtime symlinks shipping inside the `.crate`), `runtime-dependencies` (the `#[ignore]`d
minimal-versions proof in `e2e.rs`), `commits` (Conventional Commits over the outgoing range), and
`cargo bench --no-run` inside the `test` job — so `mise run test` alone is weaker than CI's.

Standing invariants:

- Output is **deterministic**: same spargen version + spec + config ⇒ byte-identical output
  (pinned by `spargen/tests/determinism.rs`).
- Generated code never silently degrades a typed schema to `serde_json::Value`, and every
  spec construct is supported, warned, or rejected — no fourth, silent behavior. New warnings
  and rejections get a stable code in `diag`, an entry in `docs/errors.md`, a cell in
  `docs/support-matrix.md`, and a fixture in `spargen/tests/frontend.rs`, in the same commit.
  The last three are enforced by tests, not convention.
- Generated output must stay consumable via `include!` — no crate-level inner attributes;
  attributes ride on emitted items.
- Prefer `pub(crate)` over `pub` for anything not part of the `build.rs` facade or an emitted
  API; module privacy plus the layering DAG is how coupling stays controlled. The DAG is enforced
  by `spargen/tests/layering.rs`, which diffs each `//! layer-deps:` header against the module's
  real `crate::` edges — the `xtask lint-layers` job `lib.rs` describes does not exist yet.

## Testing strategy (by subsystem)

Tests live closest to what they pin; when you touch a subsystem, extend its suite:

| Subsystem | Suite | What to cover |
| --- | --- | --- |
| `oas31` (+ `source`) | `spargen/tests/frontend.rs` | One minimal inline-spec fixture per diagnostic code (rejections assert `Outcome::Rejected` + code; warnings assert the code fires and generation still succeeds). `check`/`generate` must stay in parity. |
| `codegen` / `emit` | `spargen/tests/e2e.rs` | Generate a module into an application-owned fixture crate and require `cargo check` + `cargo clippy -D warnings` on it; extend the inline spec when emitting new constructs so they are compile-verified. |
| `codegen` (determinism) | `spargen/tests/determinism.rs` | Byte-identical double generation. |
| build cache | `spargen/src/cache.rs` | Complete input fingerprints plus missing, stale, and manually edited output invalidation. |
| `diag` | `spargen/src/diag/code.rs` tests | Code string round-trips; every code has title + explain text. |
| `name` | in-module proptests | Determinism, injectivity in scope, valid identifiers, keyword escaping. |
| `compat` | in-module + `carve.rs` + `e2e.rs` | Omit rules match/apply, fingerprint stability (same profile repeats, different profiles differ, order-sensitive), `W009`/`E019` in-module, `E020` in `carve.rs`. |
| `support-runtime` | in-file `#[cfg(test)]` mods | URL building, auth attachment (all schemes + alternatives + failure modes), status classification, error taxonomy semantics. No async runtime: poll-once with `Waker::noop`. |
| whole tool | `examples/petstore` + `examples/petstore-macro` (`mise run example`) | The generated client driven over real HTTP against a local mock server (params, bodies, auth, typed errors, undocumented statuses), via both the `build.rs` and macro paths; the macro run also asserts spargen stays out of the runtime graph (`cargo tree -e no-proc-macro`). |
| corpus | `spargen/tests/corpus_manifest.rs` / `mise run corpus-smoke` | `corpus/manifest.toml` is the single source of expectations (`expect = "generate"` / `"reject:E###"`); update them only with a reviewed reason. The suite drives every case, verifies each file against its pinned `sha256`, and holds the smoke task, the CI job, `snapshot.rs`, and `corpus/README.md` to the manifest — adding a case means adding it everywhere. |
| `compat` (carve) | `spargen/tests/carve.rs` | Omit-profile globbing and auto-carve: rules match what they say, carve reaches a fixpoint, and it stays deterministic. |
| `surface` | `spargen/tests/diff.rs` + in-module | `spargen diff` semver classification per change kind, and stability of the same pair twice. Every `ChangeKind` needs a fixture in `diff.rs`; the in-module tests enforce that, and pin the impact policy and the kebab-case codes (`ChangeKind` is `#[non_exhaustive]`, so only an in-crate test can notice a new variant). |
| `config` / CLI | `spargen/tests/config.rs`, `spargen/tests/cli.rs` | Config discovery and precedence, `spargen deps` output, the Cargo-integration policy, and the subcommand set (`generate` is deliberately absent). |
| lowering invariants | `spargen/tests/lowering_props.rs` | Proptests over union/`allOf` lowering: category disjointness, closed-object disjointness, exact `allOf` merge. |
| robustness | `spargen/tests/fuzz_frontend.rs` | Fixed-seed proptest no-panic harness over `check` (arbitrary bytes, UTF-8, keyword-biased and valid-skeleton documents, deep `$ref` chains), through both parsers. `fuzz/` is the nightly libFuzzer counterpart, run by hand. |
| snapshots | `spargen/tests/snapshot.rs` | One per corpus case (enforced by `corpus_manifest.rs`): the outcome plus a sorted diagnostic histogram, and an API surface for the small generating cases. Deliberately **not** the full emitted source — a change to signatures, derives, serde attributes, or the embedded runtime produces no diff here. |
| framework round-trip | `spargen/tests/recipes.rs` | The OpenAPI documents utoipa / aide / poem-openapi actually emit. |
| `emit` | in-module | The provenance header's format and version stamp, that it precedes the module verbatim and is comments-only (generated output is `include!`d, so an inner attribute here breaks every consumer), the one-file rule, and `EmitError`'s display/source chain. |
| `support` + layering | `spargen/tests/layering.rs` | Each subsystem's `//! layer-deps:` header matches the `crate::` edges it actually takes and the DAG table in `lib.rs`; each runtime source carries `#[cfg(test)]` at most once with nothing after it (the embed splits on that literal); generated output carries no test module; `runtime_files()` and the `src/support/runtime/` symlinks equal the `support-runtime/src` file set. |
| docs ↔ code | `spargen/src/diag/code.rs` tests | `docs/errors.md` lists exactly `Code::all()` with matching titles; every code a support document cites is real, every declared code appears in `docs/support-matrix.md`, and it sits in the column matching its severity. `Code::all()` is checked against the enum, and every code must be asserted by `frontend.rs` or by the suite named in that test's `OWNED_ELSEWHERE` table. |

Bug-fix discipline: every bug becomes a fixture (usually in `frontend.rs` or the runtime test
mods) *before* its fix, so regressions cannot reappear silently.

## Commits

Commits MUST follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
`fix:`, `chore:`, …) — enforced by `convco` at commit time, on pre-push, and in CI. Merge
commits are exempt.

## Releases

Releases are driven by release-plz: it maintains a version-bump pull request, and merging that
PR tags the release and publishes to crates.io. Never bump the version or tag manually. The
semver surface is the public API of generated output.

Publishing runs strictly in CI via crates.io Trusted Publishing (OIDC) — no
`CARGO_REGISTRY_TOKEN` secret; `release-plz.yml` mints a short-lived token with
`rust-lang/crates-io-auth-action`. Bootstrap was one-time: `0.1.0` was published manually to
create the crate, then a Trusted Publisher (`getkono/spargen`, workflow `release-plz.yml`) was
configured in the crate settings. The published crate must stay self-contained — the runtime
sources are reached through `spargen/src/support/runtime/` symlinks so they ship inside the
`.crate`; the CI `package` job (`cargo publish --dry-run`) enforces this.

`spargen-macro` is a second published crate (it depends on `spargen`, so release-plz publishes
`spargen` first). Its one-time bootstrap is complete: `0.2.0` was published manually, and its
crate settings trust `getkono/spargen`'s `release-plz.yml` workflow. CI fully verifies the macro
artifact on ordinary changes; on release PRs, release-plz performs that verification after the new
`spargen` dependency reaches the registry. Subsequent releases publish via OIDC.
