# spargen

A compile-time-correct Rust client generator for OpenAPI 3.1.x. The [`README.md`](README.md)
carries the product contract; [`docs/support-matrix.md`](docs/support-matrix.md) and
[`docs/errors.md`](docs/errors.md) are the operational surface — read them before non-trivial
changes.

## Workspace

- `spargen/` — the one published crate (library + `cli`-gated binary). Internally partitioned
  into subsystems with a declared dependency DAG: `diag`, `source`, `ir`, `oas31`, `name`,
  `support`, `codegen`, `emit`, `compat`, `cli`, and the `lib.rs` facade. Every subsystem
  `mod.rs` declares its allowed dependencies in a `//! layer-deps:` header — keep those honest.
- `support-runtime/` — the freestanding runtime embedded verbatim into generated output.
  `publish = false`; its dependencies are exactly `reqwest` / `serde` / `serde_json` / `bytes` /
  `secrecy`. No spargen crate may ever appear in a consumer's runtime graph. Each source file
  keeps its `#[cfg(test)]` module last — everything above that marker is embedded into generated
  code and must compile there.
- `examples/petstore/` — the end-to-end example (own workspace); `mise run example` must stay
  green.

## Quality

Validate changes:

```bash
mise run check   # cargo check --workspace --all-features
mise run fmt     # cargo fmt --all
mise run lint    # cargo clippy --workspace --all-targets --all-features -- -D warnings
mise run test    # cargo test --workspace --all-features
```

Standing invariants:

- Output is **deterministic**: same spargen version + spec + config ⇒ byte-identical output
  (pinned by `spargen/tests/determinism.rs`).
- Generated code never silently degrades a typed schema to `serde_json::Value`, and every
  spec construct is supported, warned, or rejected — no fourth, silent behavior. New warnings
  and rejections get a stable code in `diag`, an entry in `docs/errors.md`, and a fixture in
  `spargen/tests/frontend.rs`, in the same commit.
- Generated output must stay consumable via `include!` — no crate-level inner attributes;
  attributes ride on emitted items.
- Prefer `pub(crate)` over `pub` for anything not part of the `build.rs` facade or an emitted
  API; module privacy plus the layering DAG is how coupling stays controlled.

## Testing strategy (by subsystem)

Tests live closest to what they pin; when you touch a subsystem, extend its suite:

| Subsystem | Suite | What to cover |
| --- | --- | --- |
| `oas31` (+ `source`) | `spargen/tests/frontend.rs` | One minimal inline-spec fixture per diagnostic code (rejections assert `Outcome::Rejected` + code; warnings assert the code fires and generation still succeeds). `check`/`generate` must stay in parity. |
| `codegen` / `emit` | `spargen/tests/e2e.rs` | Generate a standalone crate and require `cargo check` + `cargo clippy -D warnings` on it; extend the inline spec when emitting new constructs so they are compile-verified. |
| `codegen` (determinism) | `spargen/tests/determinism.rs` | Byte-identical double generation. |
| `emit` (`--check`) | `spargen/tests/drift.rs` | Clean / drifted / missing contract and `W004`. |
| `diag` | `spargen/src/diag/code.rs` tests | Code string round-trips; every code has title + explain text. |
| `name` | in-module proptests | Determinism, injectivity in scope, valid identifiers, keyword escaping. |
| `compat` | in-module + `e2e.rs` | Omit rules match/apply, fingerprint stability, `W009`/`E019`/`E020`. |
| `support-runtime` | in-file `#[cfg(test)]` mods | URL building, auth attachment (all schemes + alternatives + failure modes), status classification, error taxonomy semantics. No async runtime: poll-once with `Waker::noop`. |
| whole tool | `examples/petstore` (`mise run example`) | The generated client driven over real HTTP against a local mock server: params, bodies, auth, typed errors, undocumented statuses. |
| corpus | `mise run corpus-smoke` / `corpus/manifest.toml` | Pinned real-world specs with expected outcomes (`expect = "generate"` / `"reject:E###"`); update expectations only with a reviewed reason. |

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
